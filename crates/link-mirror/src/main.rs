//! `andromeda-link-mirror`: a phone's screen in a window, from `andromeda-linkd`'s `OpenMirror`, with touches, the
//! wheel, keys, and the phone's buttons going back as `MirrorInput`, and its sound on the speakers. `--frames` decodes
//! to PNG files and a WAV file instead, `--luma` to each frame's brightness and time, for tests.

mod input;
mod keys;
mod stream;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Parser;
use gtk4::prelude::*;
use gtk4::{gio, glib};

const LINK: &str = "org.andromeda.Link1";
const LINK_PATH: &str = "/org/andromeda/Link1";
/// The phone asks the user first; the daemon gives up after 60 s.
pub const OPEN_TIMEOUT_MS: i32 = 90_000;

#[derive(Parser)]
#[command(about = "Mirror a paired phone's screen")]
struct Cli {
    /// The phone's Link device id.
    device: String,
    /// The window's title.
    #[arg(long, default_value = "Phone")]
    name: String,
    /// Decode into DIR/frame-NNN.png instead of a window, then exit.
    #[arg(long, value_name = "DIR")]
    frames: Option<PathBuf>,
    /// Decode without a window and write each frame's decode time (Unix ms) and mean luma to FILE, then exit.
    #[arg(long, value_name = "FILE", conflicts_with = "frames")]
    luma: Option<PathBuf>,
    /// With --frames or --luma, how many access units to decode.
    #[arg(long, default_value_t = 60)]
    count: u32,
    /// Offer the phone no codec besides H.264, for a comparison run.
    #[arg(long)]
    h264: bool,
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).format_timestamp(None).init();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    gstreamer::init()?;
    let bus = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)?;
    let offered = if cli.h264 { Vec::new() } else { stream::offered(cli.frames.is_some()) };
    let (socket, width, height, codec) = open(&bus, &cli.device, &offered)?;
    log::info!("mirroring {} at {width}x{height} in {codec:?}", cli.device);
    if let Some(dir) = cli.frames {
        return stream::to_png(socket, codec, &dir, cli.count);
    }
    if let Some(file) = cli.luma {
        return stream::to_luma(socket, codec, &file, cli.count);
    }
    // The paintable sink needs a GDK display before its pipeline starts.
    gtk4::init()?;
    let display = std::rc::Rc::new(stream::Display::new());
    let socket = std::cell::Cell::new(Some(socket));
    let app = gtk4::Application::builder().application_id("org.andromeda.LinkMirror").build();
    let (device, name) = (cli.device, cli.name);
    app.connect_activate(move |app| {
        if let Some(socket) = socket.take() {
            input::window(app, &bus, &device, &name, &display, socket, codec, width, height);
        }
    });
    let status = app.run_with_args::<&str>(&[]);
    if status == glib::ExitCode::SUCCESS { Ok(()) } else { bail!("the window ended with {status:?}") }
}

/// Calls `OpenMirrorWith`, offering `codecs` besides H.264; it answers once the phone's user allowed capture.
fn open(bus: &gio::DBusConnection, device: &str, codecs: &[&str]) -> anyhow::Result<Opened> {
    let reply = bus
        .call_with_unix_fd_list_sync(
            Some(LINK),
            LINK_PATH,
            LINK,
            "OpenMirrorWith",
            Some(&(device, codecs).to_variant()),
            Some(glib::VariantTy::new("(huus)")?),
            gio::DBusCallFlags::NONE,
            OPEN_TIMEOUT_MS,
            None::<&gio::UnixFDList>,
            None::<&gio::Cancellable>,
        )
        .context("the phone did not start mirroring")?;
    opened(&reply, codecs)
}

/// The stream, its size and its codec.
pub type Opened = (std::os::unix::net::UnixStream, u32, u32, stream::Codec);

/// `OpenMirrorWith`'s reply; a codec other than H.264 or one of `codecs` is refused.
pub fn opened((reply, fds): &(glib::Variant, Option<gio::UnixFDList>), codecs: &[&str]) -> anyhow::Result<Opened> {
    let (handle, width, height, name) =
        reply.get::<(glib::variant::Handle, u32, u32, String)>().context("a malformed OpenMirrorWith reply")?;
    let codec = stream::Codec::from_name(&name)
        .filter(|codec| *codec == stream::Codec::H264 || codecs.contains(&name.as_str()))
        .with_context(|| format!("the phone chose codec {name:?}, which was not offered"))?;
    let fds = fds.as_ref().context("OpenMirrorWith passed no descriptor")?;
    let fd = fds.get(handle.0).context("OpenMirrorWith's descriptor is missing")?;
    Ok((std::os::unix::net::UnixStream::from(fd), width, height, codec))
}
