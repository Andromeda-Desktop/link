//! The one owner of link-core's file calls that differ between Unix and Windows.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{FileExt as _, OpenOptionsExt as _};
#[cfg(windows)]
use std::os::windows::fs::{FileExt as _, MetadataExt as _, OpenOptionsExt as _};

/// Free bytes for this user and total bytes on the volume that holds `dir`.
pub struct Space {
    pub free: u64,
    pub total: u64,
}

/// Makes a created file readable by its owner only. On Windows the file inherits the ACL of its folder, which in
/// the user's profile grants only the user, SYSTEM and Administrators.
pub fn private(options: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    options.mode(0o600);
    options
}

/// Opens `path` and refuses a symbolic link (on Windows, any reparse point) as its last component.
pub fn open_nofollow(options: &mut OpenOptions, path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed()).open(path)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT};
        let file = options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a reparse point"));
        }
        Ok(file)
    }
}

/// Reads at `offset` without the file cursor: tasks may read one file at different offsets at once.
pub fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        file.read_at(buf, offset)
    }
    #[cfg(windows)]
    {
        file.seek_read(buf, offset)
    }
}

pub fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        file.write_all_at(buf, offset)
    }
    #[cfg(windows)]
    {
        let (mut buf, mut offset) = (buf, offset);
        while !buf.is_empty() {
            match file.seek_write(buf, offset)? {
                0 => return Err(io::ErrorKind::WriteZero.into()),
                written => {
                    buf = &buf[written..];
                    offset += written as u64;
                }
            }
        }
        Ok(())
    }
}

pub fn space(dir: &Path) -> io::Result<Space> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs(dir)?;
        Ok(Space {
            free: stat.f_bavail.saturating_mul(stat.f_frsize),
            total: stat.f_blocks.saturating_mul(stat.f_frsize),
        })
    }
    #[cfg(windows)]
    {
        windows::space(dir)
    }
}

/// Renames `from` to `to`, failing with `AlreadyExists` when `to` exists.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let (cwd, flags) = (rustix::fs::CWD, rustix::fs::RenameFlags::NOREPLACE);
        rustix::fs::renameat_with(cwd, from, cwd, to, flags).map_err(io::Error::from)
    }
    #[cfg(windows)]
    {
        windows::rename_noreplace(from, to)
    }
}

#[cfg(windows)]
#[allow(unsafe_code, reason = "FFI: kernel32 calls with NUL-terminated wide paths that outlive each call")]
mod windows {
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, MOVEFILE_WRITE_THROUGH, MoveFileExW};

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    pub fn space(dir: &Path) -> io::Result<super::Space> {
        let dir = wide(dir);
        let (mut free, mut total) = (0, 0);
        // SAFETY: `dir` is NUL-terminated and lives across the call; the out pointers are valid u64s or null.
        if unsafe { GetDiskFreeSpaceExW(dir.as_ptr(), &raw mut free, &raw mut total, std::ptr::null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(super::Space { free, total })
    }

    /// Without `MOVEFILE_REPLACE_EXISTING` the move fails when `to` exists.
    pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (wide(from), wide(to));
        // SAFETY: both paths are NUL-terminated and live across the call.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Result = std::result::Result<(), Box<dyn std::error::Error>>;

    fn scratch(name: &str) -> io::Result<std::path::PathBuf> {
        let dir = std::env::temp_dir().join(format!("link-sys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir)?;
        Ok(dir)
    }

    #[test]
    fn a_write_through_a_symlink_is_refused() -> Result {
        let dir = scratch("nofollow")?;
        let (target, link) = (dir.join("target"), dir.join("link"));
        std::fs::write(&target, b"kept")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &link)?;
        let opened = open_nofollow(OpenOptions::new().write(true), &link);
        if let Ok(file) = &opened {
            write_all_at(file, b"lost", 0)?;
        }
        assert!(opened.is_err());
        assert_eq!(std::fs::read(&target)?, b"kept");
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn a_rename_never_replaces() -> Result {
        let dir = scratch("noreplace")?;
        let (from, to) = (dir.join("from"), dir.join("to"));
        std::fs::write(&from, b"new")?;
        std::fs::write(&to, b"old")?;
        let refused = rename_noreplace(&from, &to).map_err(|error| error.kind());
        assert_eq!(refused, Err(io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(&to)?, b"old");
        std::fs::remove_file(&to)?;
        rename_noreplace(&from, &to)?;
        assert_eq!(std::fs::read(&to)?, b"new");
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
