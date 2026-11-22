//! What the mount asks `andromeda-linkd` over D-Bus: the paired phones, their folders, and bytes of their files.

use fuser::Errno;

const SERVICE: &str = "org.andromeda.Link1";
const PATH: &str = "/org/andromeda/Link1";
const INTERFACE: &str = "org.andromeda.Link1";
/// Above linkd's own 15 s answer wait (link-daemon browse.rs `ANSWER_TIMEOUT`), so its error arrives first; a hung
/// linkd then fails the call with EIO instead of holding a FUSE thread forever. zbus has no deadline of its own
/// here: the property cache's `GetAll` ignores `method_timeout`.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// linkd answers its device list from memory, without a phone round trip.
const LOCAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub struct Device {
    pub id: String,
    pub name: String,
    pub connected: bool,
}

pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Clone)]
pub struct Link {
    proxy: zbus::Proxy<'static>,
}

impl Link {
    pub async fn connect() -> zbus::Result<Self> {
        let bus = zbus::Connection::session().await?;
        Ok(Self { proxy: zbus::Proxy::new_owned(bus, SERVICE, PATH, INTERFACE).await? })
    }

    pub async fn devices(&self) -> Result<Vec<Device>, Errno> {
        let devices: Vec<(String, String, bool)> = bounded(LOCAL_TIMEOUT, self.proxy.get_property("Devices")).await?;
        Ok(devices.into_iter().map(|(id, name, connected)| Device { id, name, connected }).collect())
    }

    pub async fn list(&self, device: &str, path: &str) -> Result<Vec<Entry>, Errno> {
        let entries: Vec<(String, bool, u64, u64)> =
            bounded(CALL_TIMEOUT, self.proxy.call("ListFiles", &(device, path))).await?;
        Ok(entries.into_iter().map(|(name, dir, size, mtime)| Entry { name, dir, size, mtime }).collect())
    }

    pub async fn stat(&self, device: &str, path: &str) -> Result<Entry, Errno> {
        let (name, dir, size, mtime): (String, bool, u64, u64) =
            bounded(CALL_TIMEOUT, self.proxy.call("StatFile", &(device, path))).await?;
        Ok(Entry { name, dir, size, mtime })
    }

    /// A PNG at most `size` pixels on its longer side.
    pub async fn thumbnail(&self, device: &str, path: &str, size: u16) -> Result<Vec<u8>, Errno> {
        bounded(CALL_TIMEOUT, self.proxy.call("Thumbnail", &(device, path, size))).await
    }

    pub async fn read(&self, device: &str, path: &str, offset: u64, length: u32) -> Result<Vec<u8>, Errno> {
        bounded(CALL_TIMEOUT, self.proxy.call("ReadFile", &(device, path, offset, length))).await
    }
}

async fn bounded<T>(limit: std::time::Duration, call: impl Future<Output = zbus::Result<T>>) -> Result<T, Errno> {
    let Ok(result) = tokio::time::timeout(limit, call).await else {
        log::warn!("andromeda-linkd: no answer in {} s", limit.as_secs());
        return Err(Errno::EIO);
    };
    result.map_err(|error| errno(&error))
}

/// The errno a failed call means to a program reading the mount.
fn errno(error: &zbus::Error) -> Errno {
    let zbus::Error::MethodError(name, message, _) = error else {
        log::warn!("andromeda-linkd: {error}");
        return Errno::EIO;
    };
    match (name.as_str(), message.as_deref()) {
        ("org.andromeda.Link1.Error.Refused", Some("not-found")) | ("org.andromeda.Link1.Error.NotConnected", _) => {
            Errno::ENOENT
        }
        ("org.andromeda.Link1.Error.Refused", Some("not-allowed" | "denied")) => Errno::EACCES,
        ("org.andromeda.Link1.Error.Refused", Some("not-a-folder")) => Errno::ENOTDIR,
        ("org.andromeda.Link1.Error.Refused", Some("not-a-file")) => Errno::EISDIR,
        ("org.andromeda.Link1.Error.Refused", Some("busy")) => Errno::EAGAIN,
        _ => {
            log::info!("andromeda-linkd: {error}");
            Errno::EIO
        }
    }
}
