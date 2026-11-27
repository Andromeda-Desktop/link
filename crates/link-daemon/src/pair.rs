//! `andromeda-linkd pair`: pairs a phone from a terminal where no Link window runs (Windows until its window ships).
//! It opens the running linkd's pairing window and prints its QR code and code, as the window shows them.

use anyhow::Context as _;
use futures_util::StreamExt as _;
use qrcodegen::{QrCode, QrCodeEcc};

/// linkd closes a pairing window after this long (Link1 `StartPairing`).
const WINDOW: std::time::Duration = std::time::Duration::from_secs(120);
/// Light modules around the code, as the QR specification asks.
const QUIET_ZONE: i32 = 4;

pub async fn pair() -> anyhow::Result<()> {
    let bus = link_api::connect().await.context("connecting to andromeda-linkd")?;
    let link = link_api::Link1Proxy::new(&bus).await?;
    let mut finished = link.receive_pairing_finished().await?;
    let mut failed = link.receive_pairing_failed().await?;
    let (code, uri) = link.start_pairing().await.context("asking andromeda-linkd (is it running?)")?;
    let qr = QrCode::encode_text(&uri, QrCodeEcc::Medium).context("the pairing link does not fit a QR code")?;
    println!("{}", terminal_text(&qr));
    println!("Scan this with Link on the phone. Code: {code}. The window closes in {} s.", WINDOW.as_secs());
    tokio::select! {
        Some(signal) = finished.next() => {
            println!("paired with {}", signal.args()?.name);
            Ok(())
        }
        Some(signal) = failed.next() => anyhow::bail!("pairing failed: {}", signal.args()?.reason),
        () = tokio::time::sleep(WINDOW) => anyhow::bail!("no phone paired within {} s", WINDOW.as_secs()),
        result = tokio::signal::ctrl_c() => {
            result?;
            link.cancel_pairing().await?;
            anyhow::bail!("cancelled")
        }
    }
}

/// `qr` as text for a terminal with light text on a dark background: light modules are drawn, two rows per line.
fn terminal_text(qr: &QrCode) -> String {
    let light = |x: i32, y: i32| !qr.get_module(x, y);
    let range = -QUIET_ZONE..qr.size() + QUIET_ZONE;
    range
        .clone()
        .step_by(2)
        .map(|y| {
            range
                .clone()
                .map(|x| match (light(x, y), light(x, y + 1)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each character holds two module rows: the text read back gives the code's modules, quiet zone included.
    #[test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    fn the_terminal_text_holds_every_module_of_the_code() {
        let qr = QrCode::encode_text("link://pair?k=abc", QrCodeEcc::Medium).expect("a code");
        let text = terminal_text(&qr);
        let side = usize::try_from(qr.size() + 2 * QUIET_ZONE).expect("a size");
        let lines: Vec<Vec<char>> = text.lines().map(|line| line.chars().collect()).collect();
        assert_eq!(lines.len(), side.div_ceil(2));
        for (y, line) in (-QUIET_ZONE..).step_by(2).zip(&lines) {
            assert_eq!(line.len(), side);
            for (x, char) in (-QUIET_ZONE..).zip(line) {
                let (top, bottom) = (!qr.get_module(x, y), !qr.get_module(x, y + 1));
                assert_eq!((matches!(char, '█' | '▀'), matches!(char, '█' | '▄')), (top, bottom), "({x}, {y})");
            }
        }
    }
}
