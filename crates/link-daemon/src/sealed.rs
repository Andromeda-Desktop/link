//! The device key, sealed by `systemd-creds --user` to this machine's TPM and host key and to this user, so that a
//! copy of the state directory does not carry it.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, bail};
use link_core::identity::Identity;

const NAME: &str = "--name=link-identity";

/// Loads the key from `sealed`; otherwise loads `plain`, or makes a new key, and seals it. When sealing fails, the key
/// stays in `plain` at mode 0600, the reason is logged, and the next start tries again.
pub fn load(sealed: &Path, plain: &Path) -> anyhow::Result<Identity> {
    if sealed.exists() {
        let pkcs8 = creds(&["decrypt", NAME, &sealed.to_string_lossy(), "-"], None)
            .with_context(|| format!("unsealing {}", sealed.display()))?;
        return Ok(Identity::from_pkcs8(pkcs8)?);
    }
    let fresh = !plain.exists();
    let identity = if fresh { Identity::generate()? } else { Identity::load_or_create(plain)? };
    match seal(identity.pkcs8(), sealed) {
        Ok(()) if fresh => log::info!("sealed a new device key in {}", sealed.display()),
        Ok(()) => {
            fs::remove_file(plain).with_context(|| format!("removing {}", plain.display()))?;
            log::info!("sealed the device key from {} in {}", plain.display(), sealed.display());
        }
        Err(error) => {
            log::warn!("the device key stays unsealed in {}: {error:#}", plain.display());
            if fresh {
                identity.save(plain)?;
            }
        }
    }
    Ok(identity)
}

fn seal(pkcs8: &[u8], sealed: &Path) -> anyhow::Result<()> {
    let partial = sealed.with_extension("partial");
    let _ = fs::remove_file(&partial);
    creds(&["encrypt", NAME, "-", &partial.to_string_lossy()], Some(pkcs8))?;
    fs::rename(&partial, sealed)?;
    Ok(())
}

fn creds(args: &[&str], input: Option<&[u8]>) -> anyhow::Result<Vec<u8>> {
    let mut child = Command::new("systemd-creds")
        .arg("--user")
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting systemd-creds")?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("systemd-creds {}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output.stdout)
}
