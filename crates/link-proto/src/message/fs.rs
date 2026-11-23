use super::*;

/// Lists a folder on the phone. `path` is `/` for the phone's roots, else `/<root>/<name>/...`; `cursor` continues a
/// listing from its `next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsList {
    pub req: u64,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsEntry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// Unix seconds.
    pub mtime: u64,
}

/// One page of a listing; `next` is the cursor for the rest, when there is more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsEntries {
    pub req: u64,
    pub entries: Vec<FsEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u32>,
}

/// Reads `len` bytes of a file from `offset`; answered by `fs-data` frames in order, the last one marked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsRead {
    pub req: u64,
    pub path: String,
    pub offset: u64,
    pub len: u32,
}

/// Puts `data` at `offset` in a file inside a folder root, creating it at offset 0; answered by `fs-entries` holding
/// the file's entry after the write, or `fs-error` (`denied` when offset 0 names a file that exists).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsWrite {
    pub req: u64,
    pub path: String,
    pub offset: u64,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

/// Removes a file inside a folder root; answered by `fs-entries` with no entries, or `fs-error`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsDelete {
    pub req: u64,
    pub path: String,
}

/// Asks the phone to take a photo with its camera; answered by `photo-taken` once the user took it or did not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotoRequest {
    pub req: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhotoRefusal {
    /// The phone's browse switch for this desktop is off.
    NotAllowed,
    /// Android does not let the app use the camera.
    Denied,
    /// The user closed the camera without a photo, or did not answer in time.
    Cancelled,
    Failed,
}

impl PhotoRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAllowed => "not-allowed",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

/// The photo's browse path under the Photos root, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotoTaken {
    pub req: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<PhotoRefusal>,
}

/// Asks for the phone's newest photos; answered by `photos-list`, or `fs-error` (`denied` while Android withholds photo access).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotosRecent {
    pub req: u64,
    pub count: u16,
    /// Absent: the newest photos alone. Present (needs `feature:photos-page`): the `count` items from `offset` on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<PhotosPage>,
}

/// Which items of the media store, newest first, a paged `photos-recent` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotosPage {
    pub offset: u32,
    pub media: PhotoMedia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhotoMedia {
    All,
    Photos,
    Videos,
}

/// A photo as `photos-list` gives it: its browse path under the Photos root, for `fs-thumb` and `fs-read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentPhoto {
    pub path: String,
    pub name: String,
    /// Unix seconds.
    pub added: u64,
    /// A video's length in seconds; absent for a photo. Only in the answer to a paged ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<u32>,
}

/// The newest photos first, at most the count asked for. A paged answer covers the asked range of the media store,
/// less the names the protocol cannot carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotosList {
    pub req: u64,
    pub photos: Vec<RecentPhoto>,
    /// The items of the asked kind in the whole media store; only in the answer to a paged ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
}

/// Asks for one entry; answered by `fs-entries` holding that entry alone, or `fs-error`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsStat {
    pub req: u64,
    pub path: String,
}

/// Asks for a PNG of an image or video at most `px` on its longer side; answered like `fs-read` from offset 0, at most
/// [`MAX_FS_READ`] bytes, or `fs-error` (`not-found` when the file has no picture).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsThumb {
    pub req: u64,
    pub path: String,
    pub px: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsData {
    pub req: u64,
    pub offset: u64,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    /// The read is complete: all it asked for, or the end of the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub last: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FsRefusal {
    NotFound,
    /// Android does not let the app read it, or the path leaves its root.
    Denied,
    /// The phone's browse switch for this desktop is off.
    NotAllowed,
    NotAFolder,
    NotAFile,
    Busy,
    Io,
}

impl FsRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::Denied => "denied",
            Self::NotAllowed => "not-allowed",
            Self::NotAFolder => "not-a-folder",
            Self::NotAFile => "not-a-file",
            Self::Busy => "busy",
            Self::Io => "io",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsError {
    pub req: u64,
    pub reason: FsRefusal,
}

pub const MAX_FS_PATH: usize = 4096;
pub const MAX_FS_NAME: usize = 255;
/// Entries per `fs-entries`, so a page of the longest names still fits a control frame.
pub const MAX_FS_ENTRIES: usize = 128;
pub const MAX_FS_READ: u32 = 1024 * 1024;
pub const MAX_FS_THUMB_PX: u16 = 1024;
/// Photos per `photos-list`, so the longest paths still fit a control frame.
pub const MAX_RECENT_PHOTOS: u16 = 64;
/// Bytes per `fs-data`, so a read never holds the control stream for long.
pub const MAX_FS_CHUNK: usize = 48 * 1024;

/// A browse path: `/`, or `/` then components that are not empty, `.`, or `..`, with no NUL.
pub fn fs_path_valid(path: &str) -> bool {
    path.len() <= MAX_FS_PATH
        && !path.contains('\0')
        && (path == "/"
            || path.strip_prefix('/').is_some_and(|rest| rest.split('/').all(|part| !matches!(part, "" | "." | ".."))))
}

pub(super) fn fs_name_valid(name: &str) -> bool {
    sized(name, 1, MAX_FS_NAME) && !name.contains('/') && !name.contains('\0') && name != "." && name != ".."
}
