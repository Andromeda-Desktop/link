//! Stands in for the app's screen capture: an Annex B H.264 file, or HEVC when it ends in `.h265` or `.hevc`, split
//! into access units at its access unit delimiters, streamed on repeat at 30 frames a second once the desktop asks.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use link_core::client::Client;
use link_core::identity::DeviceId;
use link_core::mirror::Unit;
use link_core::proto::message::{Codec, MIRROR_CONFIG, MIRROR_KEYFRAME};

const FRAME_US: u64 = 33_333;
const FRAME: Duration = Duration::from_micros(FRAME_US);
const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_AUD: u8 = 9;
/// HEVC: IDR and CRA pictures, which a decoder starts at.
const HEVC_IRAP: std::ops::RangeInclusive<u8> = 16..=21;
const HEVC_VPS: u8 = 32;
const HEVC_AUD: u8 = 35;

#[derive(Clone)]
pub struct Source {
    pub file: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// Streams until the desktop goes or the task is aborted.
pub async fn stream(client: Client, desktop: DeviceId, source: Source) -> anyhow::Result<()> {
    let ext = source.file.extension().and_then(|ext| ext.to_str());
    let hevc = matches!(ext, Some("h265" | "hevc"));
    let units = units(&std::fs::read(&source.file).context("reading --mirror-file")?, hevc);
    anyhow::ensure!(!units.is_empty(), "--mirror-file holds no access unit delimiters");
    let encoders: &[Codec] = if hevc { &[Codec::Hevc] } else { &[] };
    let (sender, codec) = client.start_mirror(desktop, source.width, source.height, encoders).await?;
    anyhow::ensure!(codec.is_some() == hevc, "the desktop did not ask for HEVC, which --mirror-file holds");
    let mut ticker = tokio::time::interval(FRAME);
    for (index, unit) in units.iter().cycle().enumerate() {
        ticker.tick().await;
        let pts_us = u64::try_from(index)? * FRAME_US;
        sender.send(&Unit { pts_us, flags: unit.0, data: unit.1.clone() }).await?;
    }
    Ok(())
}

/// Each access unit with its flags: keyframe when it holds an IDR (HEVC: IRAP) picture, configuration when it holds
/// an SPS (HEVC: VPS).
fn units(bytes: &[u8], hevc: bool) -> Vec<(u8, Vec<u8>)> {
    let aud = if hevc { HEVC_AUD } else { NAL_AUD };
    let starts: Vec<usize> = nal_starts(bytes, hevc).filter(|&(_, nal)| nal == aud).map(|(at, _)| at).collect();
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).copied().unwrap_or(bytes.len());
            let unit = &bytes[start..end];
            let mut flags = 0;
            for (_, nal) in nal_starts(unit, hevc) {
                flags |= match (hevc, nal) {
                    (false, NAL_IDR) => MIRROR_KEYFRAME,
                    (false, NAL_SPS) | (true, HEVC_VPS) => MIRROR_CONFIG,
                    (true, nal) if HEVC_IRAP.contains(&nal) => MIRROR_KEYFRAME,
                    _ => 0,
                };
            }
            (flags, unit.to_vec())
        })
        .collect()
}

/// Where each NAL unit's start code begins, and its type.
fn nal_starts(bytes: &[u8], hevc: bool) -> impl Iterator<Item = (usize, u8)> + '_ {
    (0..bytes.len().saturating_sub(3)).filter_map(move |at| {
        let long = bytes[at..].starts_with(&[0, 0, 0, 1]);
        let short = !long && bytes[at..].starts_with(&[0, 0, 1]) && (at == 0 || bytes[at - 1] != 0);
        let header = if long { at + 4 } else { at + 3 };
        (long || short)
            .then(|| bytes.get(header).map(|nal| (at, if hevc { (nal >> 1) & 0x3f } else { nal & 0x1f })))
            .flatten()
    })
}
