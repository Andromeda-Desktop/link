use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkKind {
    Wifi,
    Cellular,
    Ethernet,
    None,
    Other,
}

/// The phone's battery and network, sent on connect and on change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    /// Percent, 0 to 100.
    pub battery: u8,
    pub charging: bool,
    pub network: NetworkKind,
    /// Cellular signal in bars, 0 to 4; absent without a SIM in service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<u8>,
    /// The user turned off the app's notification access; nothing is mirrored until it is on again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notifications_off: bool,
    /// [`SYSTEM_CONTROL`]: media volume, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_volume: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ringer: Option<Ringer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dnd: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    /// [`STATUS_STORAGE`]: the phone's primary shared storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<Storage>,
}

/// Bytes of a phone's primary shared storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub free: u64,
    pub total: u64,
}

impl Status {
    pub(super) fn valid(&self) -> bool {
        self.battery <= 100
            && self.signal.is_none_or(|bars| bars <= 4)
            && self.media_volume.is_none_or(|percent| percent <= 100)
            && self.storage.is_none_or(|storage| storage.free <= storage.total)
    }

    /// The status for a desktop without [`SYSTEM_CONTROL`].
    #[must_use]
    pub fn without_control(self) -> Self {
        Self { media_volume: None, ringer: None, dnd: None, locked: None, ..self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ringer {
    Normal,
    Vibrate,
    Silent,
}

impl Ringer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Vibrate => "vibrate",
            Self::Silent => "silent",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Normal, Self::Vibrate, Self::Silent].into_iter().find(|ringer| ringer.as_str() == name)
    }
}

impl NetworkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Cellular => "cellular",
            Self::Ethernet => "ethernet",
            Self::None => "none",
            Self::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "wifi" => Self::Wifi,
            "cellular" => Self::Cellular,
            "ethernet" => Self::Ethernet,
            "none" => Self::None,
            "other" => Self::Other,
            _ => return None,
        })
    }
}
