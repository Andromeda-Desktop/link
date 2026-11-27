//! `link-session` on Windows: the daemon's signals served with Win32 backends: keep-awake, the PC's state (battery,
//! lock, presence, display) with lock and sleep, the output volume, and the phone as touchpad and keyboard.
//! Notifications, clipboard, ring and the screen come with their Windows backends (TASK-link-windows W6, W7).

#![allow(unsafe_code, reason = "FFI: COM initialization for this thread")]

use anyhow::Context;
use futures_util::StreamExt;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use crate::{
    attempt, awake, input, link, pump_input, report, report_status, system, system_command, volume, volume_changed,
};

pub async fn run() -> anyhow::Result<()> {
    // The volume backend's COM objects live in the multithreaded apartment; this thread runs the session loop.
    // SAFETY: once, before any COM call on this thread; never uninitialized, so COM lives as long as the process.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().context("initializing COM")?;
    // The daemon's named pipe, which closes when the daemon stops.
    let daemon = link_api::connect().await.context("connecting to andromeda-linkd")?;
    let link = link::Link::connect(&daemon).await?;
    let mut system_commands = link.proxy.receive_system_command().await?;
    let mut remote_input = link.proxy.receive_remote_input().await?;
    let mut keep_awake = link.proxy.receive_keep_awake_changed().await;
    let mut awake = awake::Awake::default();
    awake.set(link.proxy.keep_awake().await.unwrap_or(false));
    let mut volume = attempt("Windows volume", volume::Volume::connect().await);
    let mut system = system::System::connect();
    let mut input = input::Backend::native();
    if let Some(backend) = &input {
        log::info!("input through {}", backend.name());
    }
    let optional = [
        (input.is_some(), "input"),
        (input.as_ref().is_some_and(input::Backend::shortcuts), "shortcuts"),
        (volume.is_some(), "volume"),
    ];
    let can: Vec<&str> =
        optional.into_iter().filter_map(|(on, name)| on.then_some(name)).chain(system.capabilities()).collect();
    log::info!("can: {}", can.join(", "));
    report(&link, &can, volume.as_ref(), &system).await;
    log::info!("running");
    loop {
        tokio::select! {
            // Unguarded: a Link1 signal stream never polled stops all delivery after 8 owner changes.
            Some(signal) = system_commands.next() => {
                let Ok(args) = signal.args() else { continue };
                let result = system_command(volume.as_ref(), &mut system, args.command, args.value).await;
                if let Err(error) = &result {
                    log::warn!("{} {}: {error}", args.command, args.value);
                }
                link.system_result(args.device_id, args.serial, result.err().as_deref()).await;
                report_status(&link, volume.as_ref(), &system).await;
            }
            Some(signal) = remote_input.next() => {
                // The daemon sends it only from devices holding the input grant.
                let Ok(args) = signal.args() else { continue };
                let Some(backend) = input.as_mut() else { continue };
                match input::Action::parse(args.action, args.dx, args.dy, args.detail.to_owned()) {
                    Some(action) => backend.run(action).context("the input backend stopped")?,
                    None => log::debug!("remote input {} ignored", args.action),
                }
            }
            result = pump_input(&mut input) => result.context("the input backend stopped")?,
            result = volume_changed(&mut volume) => match result {
                Ok(()) => report_status(&link, volume.as_ref(), &system).await,
                Err(error) => log::warn!("watching the volume: {error:#}"),
            },
            () = system.changed() => report_status(&link, volume.as_ref(), &system).await,
            Some(change) = keep_awake.next() => awake.set(change.get().await.unwrap_or(false)),
            () = daemon.closed() => anyhow::bail!("andromeda-linkd closed the connection"),
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}
