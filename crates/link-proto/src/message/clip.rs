use super::*;

pub const MAX_CLIP_MIMES: usize = 16;
pub const MAX_CLIP_SIZE: u64 = 64 << 20;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipOffer {
    pub id: u64,
    pub mimes: Vec<String>,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// [`CLIP_ITEMS`]: each type's length, in `mimes` order; the first is `size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<u64>>,
    /// [`CLIP_ITEMS`]: the copy's random id, kept when a side offers a received clip on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<u64>,
    /// [`CLIP_ITEMS`]: when the copy was made, in ms since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

/// The body of `clip-pull`, and of `clip-data`, the first frame of the stream that answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipPull {
    pub id: u64,
    pub mime: String,
}

impl ClipOffer {
    pub fn is_valid(&self) -> bool {
        let mimes_ok = (1..=MAX_CLIP_MIMES).contains(&self.mimes.len())
            && self.mimes.iter().all(|mime| (1..=MAX_MIME_LEN).contains(&mime.len()));
        let text_ok = self.text.as_ref().is_none_or(|text| {
            (1..=MAX_SHARE_LEN).contains(&text.len()) && self.mimes.iter().any(|mime| is_text_mime(mime))
        });
        let sizes_ok = self.sizes.as_ref().is_none_or(|sizes| {
            sizes.len() == self.mimes.len()
                && sizes.first() == Some(&self.size)
                && sizes.iter().all(|size| *size <= MAX_CLIP_SIZE)
        });
        mimes_ok && text_ok && sizes_ok && self.size <= MAX_CLIP_SIZE
    }

    /// The length of type `mime`: from `sizes`, or `size` for an offer without them.
    pub fn size_of(&self, mime: &str) -> Option<u64> {
        let index = self.mimes.iter().position(|offered| offered == mime)?;
        Some(self.sizes.as_ref().and_then(|sizes| sizes.get(index).copied()).unwrap_or(self.size))
    }

    /// The offer for a peer without [`CLIP_ITEMS`]: only the first type, since its `size` is the only one it knows.
    #[must_use]
    pub fn without_items(mut self) -> Self {
        self.mimes.truncate(1);
        if !self.mimes.iter().any(|mime| is_text_mime(mime)) {
            self.text = None;
        }
        Self { sizes: None, origin: None, at: None, ..self }
    }
}

/// `text/plain`, with or without parameters.
pub fn is_text_mime(mime: &str) -> bool {
    mime.split(';').next().is_some_and(|base| base.trim().eq_ignore_ascii_case("text/plain"))
}
