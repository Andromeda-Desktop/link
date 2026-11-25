//! The phone as the app runs it: a present session through `link_core::client`, shares and files both ways.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use link_core::Error;
use link_core::client::{self, Client, ClientEvent};
use link_core::identity::DeviceId;
use link_core::inbox::Inbox;
use link_core::outbox::Sent;
use link_core::phone::{Phone, Session};
use link_core::proto::message::{Message, NetworkKind, Share, ShareKind, Status, Storage};
use link_core::reach::Via;
use link_core::transfer::{ClipBytes, LocalClip, TransferEvent};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal::unix::{SignalKind, signal};

use crate::held::{self, Held};
use crate::{OnOffer, files};

/// Stays present until SIGTERM, SIGINT, or `seconds`, printing every event as a JSON line. Each stdin line is a
/// command: `<text|link> <text>` shares with the desktop, the transfer, clipboard, and status commands are in
/// `command_line`, and the rest in [`Held::command`].
pub struct Options {
    pub seconds: Option<u64>,
    pub on_offer: OnOffer,
    pub status: Option<Status>,
    pub mirror: Option<crate::mirror::Source>,
    pub messages: Option<std::sync::Arc<dyn link_core::messages::Messages>>,
}

pub async fn hold(phone: Phone, inbox: Inbox, id: DeviceId, options: Options) -> anyhow::Result<()> {
    let Options { seconds, on_offer, status, mirror, messages } = options;
    let (client, actor, mut events) = client::client(phone, inbox);
    let drive = async move {
        if let Some(status) = status {
            client.set_status(status).await?;
        }
        client.set_messages(messages).await?;
        client.set_present(true).await?;
        let mut held = Held::new(client.clone(), id.clone(), mirror);
        let mut terminate = signal(SignalKind::terminate())?;
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut stdin_open = true;
        let deadline = seconds.map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds));
        loop {
            tokio::select! {
                Some(event) = events.recv() => {
                    println!("{}", describe(&event));
                    if let ClientEvent::Transfer(TransferEvent::Speaker { from, stream }) = &event
                        && let Some(receiver) = stream.take()
                    {
                        tokio::spawn(count_speaker(from.clone(), receiver));
                    }
                    if let ClientEvent::Message { from, message: Message::MicRequest } = &event {
                        tokio::spawn(send_mic(client.clone(), from.clone()));
                    }
                    answer(&client, &event, on_offer).await;
                    held.on_event(&event).await;
                }
                line = lines.next_line(), if stdin_open => match line? {
                    Some(line) if line.starts_with("text ") || line.starts_with("link ") => {
                        println!("{}", share_line(&client, &id, &line).await);
                    }
                    Some(line) => match command_line(&client, &id, &line).await {
                        Some(result) => println!("{result}"),
                        None => println!("{}", held.command(&line).await),
                    },
                    None => stdin_open = false,
                },
                _ = terminate.recv() => break,
                _ = tokio::signal::ctrl_c() => break,
                () = sleep_until(deadline) => break,
            }
        }
        anyhow::Ok(())
    };
    let ((), result) = tokio::join!(actor.run(), drive);
    result
}

async fn answer(client: &Client, event: &ClientEvent, on_offer: OnOffer) {
    let ClientEvent::Transfer(TransferEvent::Offered { id, .. }) = event else { return };
    let accept = match on_offer {
        OnOffer::Accept => true,
        OnOffer::Decline => false,
        OnOffer::Ask => return,
    };
    if let Err(error) = client.decide(*id, accept).await {
        log::warn!("answering {id}: {error}");
    }
}

/// Shares once on demand and prints the outcome.
pub async fn share(phone: Phone, inbox: Inbox, id: DeviceId, share: Share) -> anyhow::Result<()> {
    let (client, actor, _events) = client::client(phone, inbox);
    let (kind, bytes) = (share.kind.as_str(), share.text.len());
    let send = async move { client.share(id, share).await };
    let ((), result) = tokio::join!(actor.run(), send);
    // This process exits next, so a queued share goes only with a later run: not shared now.
    if result.context("sharing")? == Sent::Queued {
        anyhow::bail!("the desktop did not take the share; it waits in the outbox");
    }
    println!("{}", json!({ "shared": kind, "bytes": bytes }));
    Ok(())
}

/// The desktop's next message other than the ones it sends by itself on every connect.
pub(crate) async fn reply(session: &mut Session) -> Result<Message, Error> {
    loop {
        match session.control.recv().await? {
            Message::Commands(_)
            | Message::Capabilities(_)
            | Message::DesktopStatus(_)
            | Message::MediaPlayer(_)
            | Message::ContactsList(_)
            | Message::TextsList(_)
            | Message::CallsList(_) => {}
            message => return Ok(message),
        }
    }
}

/// Sends `share` without checking it first, as a hostile phone would, and prints what the desktop answered.
pub async fn share_unchecked(mut phone: Phone, id: DeviceId, share: Share) -> anyhow::Result<()> {
    let mut session = phone.connect(&id).await.context("connecting")?;
    session.control.send(Message::Share(share)).await?;
    let line = match reply(&mut session).await {
        Ok(Message::ShareAck(_)) => json!({ "accepted": true }),
        Ok(other) => bail!("desktop answered {}", other.kind()),
        Err(error) => json!({ "accepted": false, "error": error.to_string() }),
    };
    session.close();
    phone.finish().await;
    println!("{line}");
    Ok(())
}

/// The transfer, clipboard, and status commands; `None` for a line that is none of them.
async fn command_line(client: &Client, id: &DeviceId, line: &str) -> Option<Value> {
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let result = match verb {
        "send" => send_line(client, id, rest).await,
        "clip" => clip_line(client, rest).await,
        "clip-text" => clip_text_line(client, rest).await,
        "status" => match parse_status(&rest.replace(' ', ",")) {
            Ok(status) => {
                client.set_status(status).await.map(|()| json!({ "event": "status-set" })).map_err(Into::into)
            }
            Err(error) => Err(error),
        },
        "pull" => pull_line(client, id, rest).await,
        "contacts-changed" => client.contacts_changed().await.map(|()| json!({ "event": verb })).map_err(Into::into),
        "texts-changed" => client.texts_changed().await.map(|()| json!({ "event": verb })).map_err(Into::into),
        "accept" | "decline" => match files::parse_transfer(rest) {
            Ok(transfer) => client.decide(transfer, verb == "accept").await.map_err(Into::into),
            Err(error) => Err(error),
        }
        .map(|answered| json!({ "event": verb, "answered": answered })),
        "cancel" => match files::parse_transfer(rest) {
            Ok(transfer) => client.cancel_transfer(transfer).await.map_err(Into::into),
            Err(error) => Err(error),
        }
        .map(|cancelled| json!({ "event": "cancel", "cancelled": cancelled })),
        _ => return None,
    };
    Some(result.unwrap_or_else(|error| json!({ "event": format!("{verb}-failed"), "error": error.to_string() })))
}

async fn send_line(client: &Client, id: &DeviceId, rest: &str) -> anyhow::Result<Value> {
    let paths: Vec<PathBuf> = rest.split_whitespace().map(PathBuf::from).collect();
    let names = files::names(&paths, &[])?;
    let sources = files::sources(&paths, names)?;
    let transfer = client.send_files(id.clone(), sources).await?;
    Ok(json!({ "event": "sending", "transfer": transfer.to_hex() }))
}

/// `<battery>,<charging 0|1>,<network>[,<signal bars>[,notifications-off]][,<key>=<value>]...`, as `--status` and a
/// `status` line take it; the signal and notifications-off may be empty. Keys: media (percent), ringer (normal,
/// vibrate, silent), dnd and locked (0 or 1), storage (`<free>/<total>` bytes).
pub fn parse_status(text: &str) -> anyhow::Result<Status> {
    let usage = "a status is <battery>,<charging 0|1>,<network>[,<signal bars>[,notifications-off]][,<key>=<value>]...";
    let (positional, keyed): (Vec<&str>, Vec<&str>) = text.split(',').partition(|part| !part.contains('='));
    let ([battery, charging, network] | [battery, charging, network, _] | [battery, charging, network, _, _]) =
        positional.as_slice()
    else {
        bail!(usage)
    };
    let (signal, off) = (positional.get(3).copied(), positional.get(4).copied());
    let mut status = Status {
        battery: battery.parse().context("battery is 0 to 100")?,
        charging: *charging == "1",
        network: NetworkKind::parse(network).context("network is wifi, cellular, ethernet, none, or other")?,
        signal: signal.filter(|bars| !bars.is_empty()).map(str::parse).transpose().context("signal is 0 to 4 bars")?,
        notifications_off: off == Some("notifications-off"),
        media_volume: None,
        ringer: None,
        dnd: None,
        locked: None,
        storage: None,
        wifi: None,
        bluetooth: None,
        torch: None,
    };
    for part in keyed {
        let (key, value) = part.split_once('=').context(usage)?;
        match key {
            "media" => status.media_volume = Some(value.parse().context("media is 0 to 100")?),
            "ringer" => {
                status.ringer = Some(serde_json::from_value(Value::String(value.to_owned())).context("ringer")?);
            }
            "dnd" => status.dnd = Some(value == "1"),
            "locked" => status.locked = Some(value == "1"),
            "wifi" => status.wifi = Some(value == "1"),
            "bluetooth" => status.bluetooth = Some(value == "1"),
            "torch" => status.torch = Some(value == "1"),
            "storage" => {
                let (free, total) = value.split_once('/').context("storage is <free>/<total> bytes")?;
                let storage =
                    Storage { free: free.parse().context("free bytes")?, total: total.parse().context("total bytes")? };
                status.storage = Some(storage);
            }
            _ => bail!("unknown status key {key}"),
        }
    }
    Ok(status)
}

/// `clip <path> <mime>... [+ <path> <mime>...]...`: offers each file's bytes as its types of the clipboard, never
/// inline, so a pull is what moves them.
async fn clip_line(client: &Client, rest: &str) -> anyhow::Result<Value> {
    let mut types = Vec::new();
    for group in rest.split(" + ") {
        let mut words = group.split_whitespace();
        let path = words.next().context("clip <path> <mime>... [+ <path> <mime>...]...")?;
        let data = std::fs::File::open(path).with_context(|| format!("opening {path}"))?;
        for mime in words {
            types.push((mime.to_owned(), ClipBytes::File(data.try_clone()?)));
        }
    }
    let mimes: Vec<String> = types.iter().map(|(mime, _)| mime.clone()).collect();
    client.offer_clip(LocalClip { types, copied: None }).await?;
    Ok(json!({ "event": "clip-sent", "mimes": mimes }))
}

/// `clip-text <text>`: offers the text as the Android app does, inline when short.
async fn clip_text_line(client: &Client, text: &str) -> anyhow::Result<Value> {
    let types = TEXT_MIMES.iter().map(|mime| ((*mime).to_owned(), ClipBytes::Text(text.to_owned()))).collect();
    client.offer_clip(LocalClip { types, copied: None }).await?;
    Ok(json!({ "event": "clip-sent", "mimes": TEXT_MIMES }))
}

const TEXT_MIMES: [&str; 2] = ["text/plain;charset=utf-8", "text/plain"];

/// `pull <clip> <mime> <path>`: writes the desktop's offered clip into a file.
async fn pull_line(client: &Client, id: &DeviceId, rest: &str) -> anyhow::Result<Value> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let [clip, mime, path] = words[..] else { bail!("pull <clip> <mime> <path>") };
    let sink = std::fs::File::create(path).with_context(|| format!("creating {path}"))?;
    let bytes = client.pull_clip(id.clone(), clip.parse()?, mime.to_owned(), sink).await?;
    Ok(json!({ "event": "pulled", "clip": clip, "mime": mime, "bytes": bytes }))
}

/// Sends an unacknowledged message without checking it and prints whether the desktop closed the connection within
/// the step timeout.
pub async fn send_unchecked(mut phone: Phone, id: DeviceId, message: Message) -> anyhow::Result<()> {
    let mut session = phone.connect(&id).await.context("connecting")?;
    session.control.send(message).await?;
    let line = match reply(&mut session).await {
        Err(Error::Timeout) => json!({ "accepted": true }),
        Ok(other) => bail!("desktop answered {}", other.kind()),
        Err(error) => json!({ "accepted": false, "error": error.to_string() }),
    };
    session.close();
    phone.finish().await;
    println!("{line}");
    Ok(())
}

async fn share_line(client: &Client, id: &DeviceId, line: &str) -> Value {
    let (kind, text) = line.split_once(' ').unwrap_or((line, ""));
    let Some(kind) = ShareKind::parse(kind) else {
        return json!({ "event": "share-failed", "error": "a line is <text|link> <text>" });
    };
    let bytes = text.len();
    match client.share(id.clone(), Share { kind, text: text.to_owned(), item: None }).await {
        Ok(Sent::Delivered) => json!({ "event": "shared", "kind": kind.as_str(), "bytes": bytes }),
        Ok(Sent::Queued) => json!({ "event": "share-queued", "kind": kind.as_str(), "bytes": bytes }),
        Err(error) => json!({ "event": "share-failed", "kind": kind.as_str(), "error": error.to_string() }),
    }
}

/// Answers `mic-request` with 3 s of Opus silence (48 kHz mono, 20 ms frames) at real time, then finishes.
async fn send_mic(client: Client, from: DeviceId) {
    /// One 20 ms mono CELT frame of silence (RFC 6716 TOC 0xF8 and an empty body).
    const SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];
    let sender = match client.start_mic(from.clone()).await {
        Ok(sender) => sender,
        Err(error) => {
            return println!("{}", json!({ "event": "mic-failed", "desktop": from, "error": error.to_string() }));
        }
    };
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let mut sent = 0_u64;
    for frame in 0..150_u64 {
        tick.tick().await;
        let unit = link_core::mirror::Unit { pts_us: frame * 20_000, flags: 4, data: SILENCE.to_vec() };
        if sender.send(&unit).await.is_err() {
            break;
        }
        sent += 1;
    }
    sender.finish().await;
    println!("{}", json!({ "event": "mic-sent", "desktop": from, "units": sent }));
}

/// Plays nothing: counts the speaker stream's Opus units and reports them when it ends.
async fn count_speaker(from: DeviceId, mut receiver: link_core::mirror::MirrorReceiver) {
    let (mut units, mut bytes) = (0_u64, 0_usize);
    let ended = loop {
        match receiver.next().await {
            Ok(Some(unit)) => {
                units += 1;
                bytes += unit.data.len();
            }
            Ok(None) => break None,
            Err(error) => break Some(error.to_string()),
        }
    };
    println!(
        "{}",
        json!({ "event": "speaker-ended", "desktop": from, "units": units, "bytes": bytes, "error": ended })
    );
}

fn describe(event: &ClientEvent) -> Value {
    match event {
        ClientEvent::Connected { desktop, addr, via, resumed } => json!({
            "event": "connected", "desktop": desktop.id, "name": desktop.name, "addr": addr, "via": via_name(*via),
            "resumed": resumed,
        }),
        ClientEvent::Disconnected { id, reason } => json!({ "event": "disconnected", "desktop": id, "reason": reason }),
        ClientEvent::Link { id, live } => json!({ "event": "link", "desktop": id, "live": live }),
        ClientEvent::Received { from, share } => {
            json!({ "event": "received", "desktop": from, "kind": share.kind.as_str(), "text": share.text })
        }
        ClientEvent::Unpaired { id } => json!({ "event": "unpaired", "desktop": id }),
        ClientEvent::Paused { id } => json!({ "event": "paused", "desktop": id }),
        ClientEvent::MirrorBlocked { from } => json!({ "event": "mirror-blocked", "desktop": from }),
        ClientEvent::Transfer(event) => files::describe(event),
        ClientEvent::Message { from, message } => held::describe(from, message),
        ClientEvent::SystemCommand { from, id, command } => json!({
            "event": "system-command", "desktop": from, "id": id, "command": command.command, "value": command.value,
        }),
    }
}

pub fn via_name(via: Via) -> &'static str {
    match via {
        Via::LastKnown => "last-known",
        Via::Mdns => "mdns",
        Via::Bluetooth => "bluetooth",
        Via::Hotspot => "hotspot",
        Via::WifiDirect => "wifi-direct",
        Via::Relay => "relay",
        Via::Usb => "usb",
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
