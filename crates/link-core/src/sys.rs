//! The one owner of link-core's file calls that differ between Unix and Windows.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, FileExt as _, OpenOptionsExt as _};
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

/// Creates `dir` and its parents; `dir` itself grants its owner only, whatever its parent grants. On Windows it gets
/// a protected DACL for the owner and SYSTEM, which everything created inside inherits.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(dir)?;
        windows::owner_only(dir)
    }
}

/// A folder of the user's profile that the user may move elsewhere.
#[cfg(windows)]
#[derive(Clone, Copy)]
pub enum KnownFolder {
    LocalAppData,
    Downloads,
}

/// Where `folder` is now, as Explorer shows it.
#[cfg(windows)]
pub fn known_folder(folder: KnownFolder) -> io::Result<PathBuf> {
    windows::known_folder(folder)
}

#[cfg(windows)]
#[allow(unsafe_code, reason = "FFI: kernel32 calls with NUL-terminated wide paths that outlive each call")]
mod windows {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    use std::path::{Path, PathBuf};
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR,
    };
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, MOVEFILE_WRITE_THROUGH, MoveFileExW};
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Downloads, FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };

    use super::KnownFolder;

    /// Full access for the object's owner (OWNER RIGHTS) and SYSTEM, inherited by files and folders; `P` drops the
    /// entries inherited from the parent.
    const OWNER_ONLY: &str = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)";

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

    pub fn owner_only(dir: &Path) -> io::Result<()> {
        let sddl: Vec<u16> = OWNER_ONLY.encode_utf16().chain([0]).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `sddl` is NUL-terminated; on success `descriptor` is a LocalAlloc block freed below.
        let made = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                null_mut(),
            )
        };
        if made == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = set_dacl(dir, descriptor);
        // SAFETY: `descriptor` came from LocalAlloc above and nothing uses it after this.
        unsafe { LocalFree(descriptor) };
        result
    }

    fn set_dacl(dir: &Path, descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
        let (mut present, mut defaulted, mut dacl) = (0, 0, null_mut::<ACL>());
        // SAFETY: `descriptor` is valid; the DACL pointer points into it, which outlives its use here.
        if unsafe { GetSecurityDescriptorDacl(descriptor, &raw mut present, &raw mut dacl, &raw mut defaulted) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let dir = wide(dir);
        let info = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        // SAFETY: `dir` is NUL-terminated and `dacl` valid across the call; owner, group and SACL stay unchanged.
        let status = unsafe {
            SetNamedSecurityInfoW(dir.as_ptr(), SE_FILE_OBJECT, info, null_mut(), null_mut(), dacl, null_mut())
        };
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status.cast_signed()));
        }
        Ok(())
    }

    pub fn known_folder(folder: KnownFolder) -> io::Result<PathBuf> {
        let id = match folder {
            KnownFolder::LocalAppData => &FOLDERID_LocalAppData,
            KnownFolder::Downloads => &FOLDERID_Downloads,
        };
        let mut path = null_mut();
        // SAFETY: `id` is a valid GUID; `path` receives a CoTaskMem string, freed below even on failure.
        let status = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT.cast_unsigned(), null_mut(), &raw mut path) };
        let result = if status == 0 {
            // SAFETY: on success `path` is a NUL-terminated wide string.
            let len = (0..usize::MAX).take_while(|&at| unsafe { *path.add(at) } != 0).count();
            // SAFETY: `path` holds `len` valid u16s.
            Ok(PathBuf::from(OsString::from_wide(unsafe { std::slice::from_raw_parts(path, len) })))
        } else {
            Err(io::Error::from_raw_os_error(status))
        };
        // SAFETY: `path` is null or the string the call allocated.
        unsafe { CoTaskMemFree(path.cast()) };
        result
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

    /// The user's temp folder grants Administrators too; the private folder drops every inherited entry.
    #[cfg(windows)]
    #[test]
    fn a_private_dir_grants_only_its_owner_and_system() -> Result {
        let dir = scratch("private")?.join("state");
        create_private_dir(&dir)?;
        let listed = std::process::Command::new("icacls").arg(&dir).output()?;
        let output = String::from_utf8_lossy(&listed.stdout).into_owned();
        assert!(listed.status.success(), "icacls failed: {output}");
        // The path itself can hold "Users" (C:\Users\...), so only the entries after it count.
        let acl = output.replace(&*dir.to_string_lossy(), "");
        assert!(acl.contains("OWNER RIGHTS") && acl.contains("NT AUTHORITY\\SYSTEM"), "missing an entry: {output}");
        assert!(
            !acl.contains("(I)") && !acl.contains("Administrators") && !acl.contains("Users"),
            "too wide: {output}"
        );
        std::fs::remove_dir_all(dir.parent().unwrap_or(&dir))?;
        Ok(())
    }
}
