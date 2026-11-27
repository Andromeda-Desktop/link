//! The device key, sealed so that a copy of the state directory does not carry it: by `systemd-creds --user` to this
//! machine's TPM and host key and to this user; on Windows by DPAPI to this user's logon credential.

use std::fs;
use std::path::Path;

use anyhow::Context;
use link_core::identity::Identity;

/// Loads the key from `sealed`; otherwise loads `plain`, or makes a new key, and seals it. When sealing fails, the key
/// stays in `plain` at mode 0600, the reason is logged, and the next start tries again.
pub fn load(sealed: &Path, plain: &Path) -> anyhow::Result<Identity> {
    if sealed.exists() {
        let pkcs8 = unseal(sealed).with_context(|| format!("unsealing {}", sealed.display()))?;
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
    seal_to(pkcs8, &partial)?;
    fs::rename(&partial, sealed)?;
    Ok(())
}

#[cfg(windows)]
fn seal_to(pkcs8: &[u8], file: &Path) -> anyhow::Result<()> {
    use std::io::Write;
    let mut out = link_core::sys::private(fs::OpenOptions::new().write(true).create_new(true)).open(file)?;
    out.write_all(&crate::dpapi::protect(pkcs8)?)?;
    out.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn unseal(sealed: &Path) -> anyhow::Result<Vec<u8>> {
    crate::dpapi::unprotect(&fs::read(sealed)?)
}

#[cfg(not(windows))]
const NAME: &str = "--name=link-identity";

#[cfg(not(windows))]
fn seal_to(pkcs8: &[u8], file: &Path) -> anyhow::Result<()> {
    creds(&["encrypt", NAME, "-", &file.to_string_lossy()], Some(pkcs8)).map(drop)
}

#[cfg(not(windows))]
fn unseal(sealed: &Path) -> anyhow::Result<Vec<u8>> {
    creds(&["decrypt", NAME, &sealed.to_string_lossy(), "-"], None)
}

#[cfg(not(windows))]
fn creds(args: &[&str], input: Option<&[u8]>) -> anyhow::Result<Vec<u8>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

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
        anyhow::bail!("systemd-creds {}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output.stdout)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    fn a_key_is_sealed_never_left_plain_and_reloads() {
        let dir = std::env::temp_dir().join(format!("link-sealed-{}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).expect("dir");
        let (sealed, plain) = (dir.join("identity.cred"), dir.join("identity"));

        let fresh = load(&sealed, &plain).expect("fresh");
        let stored = fs::read(&sealed).expect("sealed file");
        let again = load(&sealed, &plain).expect("reload");

        let (migrated_sealed, migrated_plain) = (dir.join("old.cred"), dir.join("old"));
        let old = Identity::generate().expect("key");
        old.save(&migrated_plain).expect("plain key");
        let migrated = load(&migrated_sealed, &migrated_plain).expect("migrate");
        let migrated_again = load(&migrated_sealed, &migrated_plain).expect("reload migrated");
        let plain_left = plain.exists() || migrated_plain.exists();
        drop(fs::remove_dir_all(&dir));

        assert!(!plain_left, "a key stayed in a plain file");
        assert!(!stored.windows(fresh.pkcs8().len()).any(|w| w == fresh.pkcs8()), "the sealed file holds the key");
        assert_eq!(again.pkcs8(), fresh.pkcs8());
        assert_eq!(migrated.pkcs8(), old.pkcs8());
        assert_eq!(migrated_again.pkcs8(), old.pkcs8());
    }
}
