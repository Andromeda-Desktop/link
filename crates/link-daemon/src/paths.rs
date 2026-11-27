use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

#[cfg(target_os = "macos")]
const HOME_STATE: &str = "Library/Application Support/Andromeda Link";
#[cfg(not(any(target_os = "macos", windows)))]
const HOME_STATE: &str = ".local/state/andromeda-link";
#[cfg(unix)]
const NO_STATE: &str = "no state directory: set XDG_STATE_HOME or HOME";
#[cfg(windows)]
const NO_STATE: &str = "no state directory: Windows names no Local AppData folder";
#[cfg(unix)]
const NO_DOWNLOADS: &str = "no download directory: set HOME";
#[cfg(windows)]
const NO_DOWNLOADS: &str = "no download directory: Windows names no Downloads folder";

pub struct Paths {
    pub state: PathBuf,
    pub identity: PathBuf,
    pub sealed_identity: PathBuf,
    pub devices: PathBuf,
    pub downloads: PathBuf,
    /// Artwork of phone players, for their MPRIS `mpris:artUrl`; created on first use.
    pub art: PathBuf,
    /// Mirrored phone notifications, one file per device; created on first use.
    pub notifications: PathBuf,
}

impl Paths {
    /// `$STATE_DIRECTORY` under systemd, else `$XDG_STATE_HOME/andromeda-link`, else `~/.local/state/andromeda-link`
    /// (on macOS `~/Library/Application Support/Andromeda Link`, on Windows `Andromeda Link` in the Local `AppData` folder).
    pub fn create() -> anyhow::Result<Self> {
        let dir = std::env::var_os("STATE_DIRECTORY")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_STATE_HOME").map(|state| PathBuf::from(state).join("andromeda-link")))
            .or_else(default_state)
            .context(NO_STATE)?;
        if !dir.exists() {
            link_core::sys::create_private_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        fs::metadata(&dir).with_context(|| format!("state directory {}", dir.display()))?;
        adopt_umbriel_state(&dir)?;
        let downloads = downloads_dir().context(NO_DOWNLOADS)?;
        Ok(Self {
            identity: dir.join("identity.pk8"),
            sealed_identity: dir.join("identity.cred"),
            devices: dir.join("devices.json"),
            art: dir.join("art"),
            notifications: dir.join("notifications"),
            state: dir,
            downloads,
        })
    }
}

#[cfg(unix)]
fn default_state() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(HOME_STATE))
}

#[cfg(windows)]
fn default_state() -> Option<PathBuf> {
    let local = link_core::sys::known_folder(link_core::sys::KnownFolder::LocalAppData);
    local.map(|local| local.join("Andromeda Link")).inspect_err(|error| log::error!("Local AppData: {error}")).ok()
}

/// Copies the identity and paired devices from `umbriel-link`, the state directory's old name, once.
/// It copies and does not move, because the sandbox mounts home read-only.
fn adopt_umbriel_state(dir: &Path) -> anyhow::Result<()> {
    let Some(old) = dir.parent().map(|parent| parent.join("umbriel-link")) else { return Ok(()) };
    if dir.join("identity.pk8").exists() || dir.join("identity.cred").exists() || !old.join("identity.pk8").exists() {
        return Ok(());
    }
    for name in ["devices.json", "identity.pk8"] {
        if old.join(name).exists() {
            fs::copy(old.join(name), dir.join(name)).with_context(|| format!("copy {name} from {}", old.display()))?;
        }
    }
    log::info!("copied state from {}", old.display());
    Ok(())
}

/// The downloads folder as [`Paths::create`] picks it.
#[cfg(unix)]
pub fn downloads_dir() -> Option<PathBuf> {
    download_dir(std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

/// `$XDG_DOWNLOAD_DIR`, else the Downloads folder wherever the user moved it.
#[cfg(windows)]
pub fn downloads_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DOWNLOAD_DIR").map(PathBuf::from).or_else(|| {
        let downloads = link_core::sys::known_folder(link_core::sys::KnownFolder::Downloads);
        downloads.inspect_err(|error| log::error!("the Downloads folder: {error}")).ok()
    })
}

/// `$XDG_DOWNLOAD_DIR`, else the one in `user-dirs.dirs`, else `~/Downloads`.
#[cfg(unix)]
fn download_dir(home: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DOWNLOAD_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = home?;
    let config = std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let listed = fs::read_to_string(config.join("user-dirs.dirs")).ok().and_then(|dirs| {
        let line = dirs.lines().find_map(|line| line.trim().strip_prefix("XDG_DOWNLOAD_DIR="))?;
        let value = line.trim_matches('"');
        Some(match value.strip_prefix("$HOME") {
            Some(rest) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(value),
        })
    });
    Some(listed.unwrap_or_else(|| home.join("Downloads")))
}
