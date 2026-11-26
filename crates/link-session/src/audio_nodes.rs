//! Link's `PipeWire` nodes, a sink "<phone> speaker", a source "<phone> microphone" and a playback stream "<phone>
//! sound" per phone, on one thread that runs a `PipeWire` loop. `GStreamer`'s `PipeWire` elements cannot make them: `pipewiresrc autoconnect=false` blocks
//! its state change until an app routes sound to it and cannot be stopped while it waits. A source is a null-audio-sink
//! node fed by a playback stream: a stream with a source class (`pipewiresink mode=provide`, `pw-loopback`) gets no
//! output ports on `PipeWire` 1.6.11 / `WirePlumber` 0.5.19, so `WirePlumber` never offers it as a default.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use gstreamer as gst;
use gstreamer_app as gst_app;
use pipewire as pw;

pub const RATE: u32 = 48_000;
/// Bytes of one S16LE sample frame in a microphone source (mono).
const MIC_STRIDE: usize = 2;
/// Bytes of one S16LE sample frame of a phone's sound (stereo).
const SOUND_STRIDE: usize = 4;

/// Decoded phone sound waiting for the graph to pull it, S16LE at `RATE`: mono for a microphone, stereo for sound.
pub type Pcm = Arc<Mutex<Jitter>>;
/// Sound a `Jitter` holds before it plays, and keeps after it drops a backlog.
const TARGET_MS: usize = 40;
/// Sound a `Jitter` holds at most.
const MAX_MS: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    Filling,
    Playing,
}

/// A jitter buffer of whole frames: silence until it holds `TARGET_MS`, then the sound; it fills again when it runs
/// dry, and above `MAX_MS` it drops the oldest sound down to `TARGET_MS`.
pub struct Jitter {
    bytes: VecDeque<u8>,
    level: Level,
    stride: usize,
}

impl Jitter {
    fn new(stride: usize) -> Self {
        Self { bytes: VecDeque::new(), level: Level::Filling, stride }
    }

    fn bytes_for(&self, ms: usize) -> usize {
        RATE as usize * ms / 1000 * self.stride
    }

    /// Queues whole frames.
    pub fn push(&mut self, pcm: &[u8]) {
        self.bytes.extend(pcm);
        if self.bytes.len() > self.bytes_for(MAX_MS) {
            let over = self.bytes.len() - self.bytes_for(TARGET_MS);
            self.bytes.drain(..over);
        }
    }

    /// Fills `out` (whole frames) with the next sound, silence where there is none.
    fn pull(&mut self, out: &mut [u8]) {
        if self.level == Level::Filling && self.bytes.len() >= self.bytes_for(TARGET_MS) {
            self.level = Level::Playing;
        }
        let taken = match self.level {
            Level::Filling => 0,
            Level::Playing => out.len().min(self.bytes.len()),
        };
        for (byte, sample) in out.iter_mut().zip(self.bytes.drain(..taken)) {
            *byte = sample;
        }
        out[taken..].fill(0);
        if taken < out.len() {
            self.level = Level::Filling;
        }
    }
}

enum Command {
    Sink { key: String, name: String, input: gst_app::AppSrc },
    Source { key: String, name: String, pcm: Pcm },
    Playback { key: String, name: String, pcm: Pcm },
    Remove(String),
    Quit,
}

/// The `PipeWire` thread; it quits when the last `Nodes` or `Node` goes.
struct Thread {
    commands: pw::channel::Sender<Command>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Thread {
    fn drop(&mut self) {
        if self.commands.send(Command::Quit).is_ok()
            && let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            log::warn!("the PipeWire nodes' thread panicked");
        }
    }
}

#[derive(Clone)]
pub struct Nodes {
    thread: Arc<Thread>,
}

/// A node on the `PipeWire` thread; dropping it removes the node.
pub struct Node {
    key: String,
    thread: Arc<Thread>,
}

impl Drop for Node {
    fn drop(&mut self) {
        drop(self.thread.commands.send(Command::Remove(std::mem::take(&mut self.key))));
    }
}

impl Nodes {
    pub fn start() -> anyhow::Result<Self> {
        let (commands, received) = pw::channel::channel();
        let (ready, started) = std::sync::mpsc::channel();
        let handle =
            std::thread::Builder::new().name("pipewire-nodes".into()).spawn(move || serve(received, &ready))?;
        started.recv().context("the PipeWire thread ended")??;
        Ok(Self { thread: Arc::new(Thread { commands, handle: Some(handle) }) })
    }

    /// Sink "<name> speaker" whose S16LE stereo sound goes into `input`; never autoconnected, so it carries only what
    /// an app or the user routes to it.
    pub fn sink(&self, id: &str, name: &str, input: gst_app::AppSrc) -> anyhow::Result<Node> {
        self.add(format!("andromeda-link-speaker-{id}"), |key| Command::Sink {
            key,
            name: format!("{name} speaker"),
            input,
        })
    }

    /// Source "<name> microphone" that plays what is pushed into the returned `Pcm`, silence when it runs dry.
    pub fn source(&self, id: &str, name: &str) -> anyhow::Result<(Node, Pcm)> {
        let pcm = Arc::new(Mutex::new(Jitter::new(MIC_STRIDE)));
        let shared = Arc::clone(&pcm);
        let node = self.add(format!("andromeda-link-mic-{id}"), |key| Command::Source {
            key,
            name: format!("{name} microphone"),
            pcm: shared,
        })?;
        Ok((node, pcm))
    }

    /// Playback stream "<name> sound" to the default sink that plays what is pushed into the returned `Pcm` (stereo).
    pub fn playback(&self, id: &str, name: &str) -> anyhow::Result<(Node, Pcm)> {
        let pcm = Arc::new(Mutex::new(Jitter::new(SOUND_STRIDE)));
        let shared = Arc::clone(&pcm);
        let node = self.add(format!("andromeda-link-sound-{id}"), |key| Command::Playback {
            key,
            name: format!("{name} sound"),
            pcm: shared,
        })?;
        Ok((node, pcm))
    }

    fn add(&self, key: String, command: impl FnOnce(String) -> Command) -> anyhow::Result<Node> {
        self.thread.commands.send(command(key.clone())).map_err(|_| anyhow::anyhow!("the PipeWire thread ended"))?;
        Ok(Node { key, thread: Arc::clone(&self.thread) })
    }
}

/// Runs the `PipeWire` loop that owns every node's stream until `Command::Quit`.
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
    let _errors = core
        .add_listener_local()
        .error(|id, _, res, message| log::warn!("PipeWire object {id}: {message} ({res})"))
        .register();
    let streams: Rc<RefCell<HashMap<String, Box<dyn std::any::Any>>>> = Rc::default();
    let quit = main_loop.downgrade();
    let _attached = commands.attach(main_loop.loop_(), move |command| {
        let (key, made) = match command {
            Command::Sink { key, name, input } => {
                let made = sink(&core, &key, &name, input).map(|made| Box::new(made) as Box<dyn std::any::Any>);
                (key, made)
            }
            Command::Source { key, name, pcm } => {
                let made = source(&core, &key, &name, pcm).map(|made| Box::new(made) as Box<dyn std::any::Any>);
                (key, made)
            }
            Command::Playback { key, name, pcm } => {
                let made = playback(&core, &key, &name, pcm).map(|made| Box::new(made) as Box<dyn std::any::Any>);
                (key, made)
            }
            Command::Remove(key) => return drop(streams.borrow_mut().remove(&key)),
            Command::Quit => {
                streams.borrow_mut().clear();
                if let Some(main_loop) = quit.upgrade() {
                    main_loop.quit();
                }
                return;
            }
        };
        match made {
            Ok(made) => drop(streams.borrow_mut().insert(key, made)),
            Err(error) => log::warn!("no PipeWire node {key}: {error}"),
        }
    });
    drop(ready.send(Ok(())));
    main_loop.run();
}

fn sink(
    core: &pw::core::CoreRc,
    key: &str,
    name: &str,
    input: gst_app::AppSrc,
) -> Result<(pw::stream::StreamRc, pw::stream::StreamListener<gst_app::AppSrc>), pw::Error> {
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CLASS => "Audio/Sink",
        *pw::keys::NODE_NAME => key,
        *pw::keys::NODE_DESCRIPTION => name,
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
    let mut position = [0; pw::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = pw::spa::sys::SPA_AUDIO_CHANNEL_FR;
    let format = format(2, position)?;
    let pod = pw::spa::pod::Pod::from_bytes(&format).ok_or(pw::Error::CreationFailed)?;
    stream.connect(pw::spa::utils::Direction::Input, None, pw::stream::StreamFlags::MAP_BUFFERS, &mut [pod])?;
    Ok((stream, listener))
}

/// Global ids of the microphone feed's output port and the source node's input port, and the link between them.
#[derive(Default)]
struct Wiring {
    feed: Option<u32>,
    source: Option<u32>,
    output: Option<u32>,
    input: Option<u32>,
    link: Option<pw::link::Link>,
}

/// What `source` keeps alive: the source node, its feed stream, and the registry watch that links them and owns the link.
/// Fields drop in order, so the link goes before the nodes it joins.
type Source = (
    pw::registry::Listener,
    pw::registry::RegistryRc,
    pw::stream::StreamListener<Pcm>,
    pw::stream::StreamRc,
    pw::node::Node,
);

/// The source node and the stream that feeds it the queued PCM, silence when the queue runs dry. `WirePlumber` ignores
/// a `target.object` that names a source and links the feed to the default sink, so the feed is not autoconnected:
/// once `WirePlumber` has given both nodes their ports, the registry watch links them.
fn source(core: &pw::core::CoreRc, key: &str, name: &str, pcm: Pcm) -> Result<Source, pw::Error> {
    let node = core.create_object::<pw::node::Node>(
        "adapter",
        &pw::properties::properties! {
            *pw::keys::FACTORY_NAME => "support.null-audio-sink",
            *pw::keys::NODE_NAME => key,
            *pw::keys::NODE_DESCRIPTION => name,
            *pw::keys::MEDIA_CLASS => "Audio/Source/Virtual",
            "audio.position" => "[ MONO ]",
            *pw::keys::OBJECT_LINGER => "false",
        },
    )?;
    let feed_name = format!("{key}-feed");
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::NODE_NAME => feed_name.as_str(),
        *pw::keys::NODE_AUTOCONNECT => "false",
    };
    let wiring = RefCell::new(Wiring::default());
    let registry = core.get_registry_rc()?;
    let watch = {
        let (core, source_name) = (core.clone(), key.to_owned());
        registry
            .add_listener_local()
            .global(move |global| {
                let Some(props) = global.props else { return };
                let mut wiring = wiring.borrow_mut();
                match global.type_ {
                    pw::types::ObjectType::Node if props.get(*pw::keys::NODE_NAME) == Some(feed_name.as_str()) => {
                        wiring.feed = Some(global.id);
                    }
                    pw::types::ObjectType::Node if props.get(*pw::keys::NODE_NAME) == Some(source_name.as_str()) => {
                        wiring.source = Some(global.id);
                    }
                    pw::types::ObjectType::Port => {
                        let owner = props.get(*pw::keys::NODE_ID).and_then(|id| id.parse().ok());
                        match props.get(*pw::keys::PORT_DIRECTION) {
                            Some("out") if owner.is_some() && owner == wiring.feed => wiring.output = Some(global.id),
                            Some("in") if owner.is_some() && owner == wiring.source => wiring.input = Some(global.id),
                            _ => return,
                        }
                    }
                    _ => return,
                }
                if let (None, Some(feed), Some(output), Some(source), Some(input)) =
                    (&wiring.link, wiring.feed, wiring.output, wiring.source, wiring.input)
                {
                    match core.create_object::<pw::link::Link>(
                        "link-factory",
                        &pw::properties::properties! {
                            *pw::keys::LINK_OUTPUT_NODE => feed.to_string(),
                            *pw::keys::LINK_OUTPUT_PORT => output.to_string(),
                            *pw::keys::LINK_INPUT_NODE => source.to_string(),
                            *pw::keys::LINK_INPUT_PORT => input.to_string(),
                            *pw::keys::OBJECT_LINGER => "false",
                        },
                    ) {
                        Ok(link) => wiring.link = Some(link),
                        Err(error) => log::warn!("no link from {feed_name} to {source_name}: {error}"),
                    }
                }
            })
            .register()
    };
    let stream = pw::stream::StreamRc::new(core.clone(), "phone microphone", props)?;
    let listener = stream.add_local_listener_with_user_data(pcm).process(fill).register()?;
    let mut position = [0; pw::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_MONO;
    let format = format(1, position)?;
    let pod = pw::spa::pod::Pod::from_bytes(&format).ok_or(pw::Error::CreationFailed)?;
    stream.connect(pw::spa::utils::Direction::Output, None, pw::stream::StreamFlags::MAP_BUFFERS, &mut [pod])?;
    Ok((watch, registry, listener, stream, node))
}

/// A playback stream that `WirePlumber` links to the default sink, playing the queued stereo PCM.
fn playback(
    core: &pw::core::CoreRc,
    key: &str,
    name: &str,
    pcm: Pcm,
) -> Result<(pw::stream::StreamListener<Pcm>, pw::stream::StreamRc), pw::Error> {
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::NODE_NAME => key,
        *pw::keys::NODE_DESCRIPTION => name,
        *pw::keys::MEDIA_NAME => name,
    };
    let stream = pw::stream::StreamRc::new(core.clone(), "phone sound", props)?;
    let listener = stream.add_local_listener_with_user_data(pcm).process(fill).register()?;
    let mut position = [0; pw::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = pw::spa::sys::SPA_AUDIO_CHANNEL_FR;
    let format = format(2, position)?;
    let pod = pw::spa::pod::Pod::from_bytes(&format).ok_or(pw::Error::CreationFailed)?;
    stream.connect(
        pw::spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut [pod],
    )?;
    Ok((listener, stream))
}

/// Fills the stream's next buffer from the jitter buffer.
fn fill(stream: &pw::stream::Stream, pcm: &mut Pcm) {
    let mut queued = pcm.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let stride = queued.stride;
    let Some(mut buffer) = stream.dequeue_buffer() else { return };
    let requested = usize::try_from(buffer.requested()).unwrap_or(usize::MAX);
    let Some(data) = buffer.datas_mut().first_mut() else { return };
    let Some(slice) = data.data() else { return };
    let frames = if requested == 0 { slice.len() / stride } else { requested.min(slice.len() / stride) };
    queued.pull(&mut slice[..frames * stride]);
    drop(queued);
    let chunk = data.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = i32::try_from(stride).unwrap_or(0);
    *chunk.size_mut() = u32::try_from(frames * stride).unwrap_or(0);
}

/// An S16LE `EnumFormat` pod at `RATE`.
fn format(channels: u32, position: [u32; pw::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize]) -> Result<Vec<u8>, pw::Error> {
    let mut info = pw::spa::param::audio::AudioInfoRaw::new();
    info.set_format(pw::spa::param::audio::AudioFormat::S16LE);
    info.set_rate(RATE);
    info.set_channels(channels);
    info.set_position(position);
    let format = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    Ok(pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(format),
    )
    .map_err(|_| pw::Error::CreationFailed)?
    .0
    .into_inner())
}

#[cfg(test)]
mod tests {
    use super::{Jitter, Level, MAX_MS, RATE, SOUND_STRIDE, TARGET_MS};

    /// Random packet arrivals (bursts and gaps) against steady graph pulls; samples count up from 1, so silence is 0
    /// and the order of the sound shows.
    #[test]
    fn jitter_buffer_keeps_order_bound_and_never_plays_while_filling() {
        let mut seed = 0x2545_f491_u64;
        let mut random = move |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        let mut jitter = Jitter::new(SOUND_STRIDE);
        let packet = RATE as usize / 50 * SOUND_STRIDE;
        let quantum = 1024 * SOUND_STRIDE;
        let (mut next, mut last_played) = (1u32, 0u32);
        for _ in 0..20_000 {
            if random(2) == 0 {
                for _ in 0..random(4) {
                    let frames: Vec<u8> = (0..packet / 4)
                        .flat_map(|_| {
                            next += 1;
                            next.to_le_bytes()
                        })
                        .collect();
                    jitter.push(&frames);
                    assert!(jitter.bytes.len() <= jitter.bytes_for(MAX_MS));
                }
            } else {
                let filling = jitter.level == Level::Filling && jitter.bytes.len() < jitter.bytes_for(TARGET_MS);
                let mut out = vec![0xaa; quantum];
                jitter.pull(&mut out);
                let samples: Vec<u32> =
                    out.chunks(4).map(|frame| u32::from_le_bytes(frame.try_into().unwrap_or_default())).collect();
                let sound = samples.iter().take_while(|sample| **sample != 0).count();
                assert!(samples[sound..].iter().all(|sample| *sample == 0), "silence inside the sound");
                if filling {
                    assert_eq!(sound, 0, "sound before the buffer held its target");
                }
                if sound < samples.len() {
                    assert_eq!(jitter.level, Level::Filling);
                }
                for sample in &samples[..sound] {
                    assert!(*sample > last_played, "sound out of order");
                    last_played = *sample;
                }
            }
        }
    }
}
