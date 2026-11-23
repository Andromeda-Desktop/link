use super::*;

/// A transfer's id: 16 random bytes chosen by the sender.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(pub [u8; TRANSFER_ID_LEN]);

pub const TRANSFER_ID_LEN: usize = 16;
pub const SHA256_LEN: usize = 32;
/// Longer names are cut to 255 bytes on receipt; this only bounds what a frame may carry.
pub const MAX_WIRE_NAME_LEN: usize = 4096;
pub const MAX_MIME_LEN: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMeta {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub mime: String,
    /// Empty only toward a peer whose hello carries [`LATE_HASH`]: the digest follows in [`FileHash`].
    #[serde(with = "serde_bytes")]
    pub sha256: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub transfer: TransferId,
    pub files: Vec<FileMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefuseReason {
    Declined,
    NoSpace,
    TooLarge,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfferReply {
    pub transfer: TransferId,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<RefuseReason>,
}

/// The body of `resume` and `cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferRef {
    pub transfer: TransferId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOffset {
    pub file: u64,
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeAt {
    pub transfer: TransferId,
    pub offsets: Vec<FileOffset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileDone {
    pub transfer: TransferId,
    pub file: u64,
    pub ok: bool,
}

/// Sender to receiver, after the last byte of a file offered with an empty digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileHash {
    pub transfer: TransferId,
    pub file: u64,
    #[serde(with = "serde_bytes")]
    pub sha256: Vec<u8>,
}

/// The first frame of a file's unidirectional stream; the bytes from `offset` to the file's size follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileData {
    pub transfer: TransferId,
    pub file: u64,
    pub offset: u64,
}

impl RefuseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declined => "declined",
            Self::NoSpace => "no-space",
            Self::TooLarge => "too-large",
            Self::Busy => "busy",
        }
    }
}

impl TransferId {
    pub fn to_hex(self) -> String {
        self.0.iter().fold(String::with_capacity(2 * TRANSFER_ID_LEN), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
    }

    pub fn parse_hex(text: &str) -> Option<Self> {
        if text.len() != 2 * TRANSFER_ID_LEN || !text.is_ascii() {
            return None;
        }
        let mut id = [0; TRANSFER_ID_LEN];
        for (index, byte) in id.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[2 * index..2 * index + 2], 16).ok()?;
        }
        Some(Self(id))
    }
}

impl fmt::Debug for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for TransferId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for TransferId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes = serde_bytes::ByteBuf::deserialize(deserializer)?;
        let id = <[u8; TRANSFER_ID_LEN]>::try_from(bytes.as_slice())
            .map_err(|_| serde::de::Error::invalid_length(bytes.len(), &"16 bytes"))?;
        Ok(Self(id))
    }
}

impl Offer {
    /// The rules of an offer, checked by the sender before sending and by the receiver while decoding.
    pub fn is_valid(&self) -> bool {
        !self.files.is_empty()
            && unique(self.files.iter().map(|file| file.id))
            && self.files.iter().all(FileMeta::is_valid)
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).fold(0, u64::saturating_add)
    }
}

impl FileMeta {
    fn is_valid(&self) -> bool {
        (1..=MAX_WIRE_NAME_LEN).contains(&self.name.len())
            && (1..=MAX_MIME_LEN).contains(&self.mime.len())
            && (self.sha256.is_empty() || self.sha256.len() == SHA256_LEN)
    }
}

pub(super) fn unique(ids: impl Iterator<Item = u64>) -> bool {
    let mut ids: Vec<u64> = ids.collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    ids.len() == count
}
